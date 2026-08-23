fn decode_runtime_type(value: Val, current: &Heap, background: &Heap) -> Result<CodecType, String> {
    decode_runtime_type_at(value, "Type", current, background)
}

fn decode_codec_properties(
    value: Val,
    current: &Heap,
    background: &Heap,
) -> Result<CodecProperties, String> {
    let view = HeapView {
        current,
        background: Some(background),
    };
    let DecodedValue::Dict(handle) = value.without_type_id().value() else {
        return Err("std/codec properties must be a Struct value".into());
    };
    let property = |name| {
        let value = view
            .dict_get_text(handle, name)
            .map_err(|error| error.to_string())?
            .ok_or_else(|| format!("std/codec properties has no {name:?} field"))?;
        view.declared_type_id(value)
            .map_err(|error| error.to_string())
    };
    Ok(CodecProperties {
        parse_by: property("parse_by")?,
        decode_by_parse: property("decode_by_parse")?,
        encode_by_display: property("encode_by_display")?,
        display_by: property("display_by")?,
        json_rename_all: Some(property("json_rename_all")?),
        json_untagged: Some(property("json_untagged")?),
    })
}

fn apply_codec_type_properties(
    schema: &mut CodecType,
    metadata: ValueRef<'_>,
    properties: &CodecProperties,
) -> Result<(), String> {
    if let Some(property_type) = properties.json_rename_all
        && let Some(property) = metadata.type_property(property_type)
    {
        let case = property
            .dict_get("case")
            .ok_or_else(|| "json RenameAll property has no case".to_owned())?;
        schema.json_rename_all = Some(case.runtime());
    }
    if let Some(property_type) = properties.json_untagged
        && let Some(property) = metadata.type_property(property_type)
    {
        schema.json_untagged = Some(property.runtime());
    }
    Ok(())
}

#[derive(Debug)]
enum CodecGraphError {
    Pending,
    Invalid(String),
}

fn assert_codec_graph_ready(
    schema: &CodecType,
    current: &Heap,
    background: &Heap,
) -> Result<(), CodecGraphError> {
    fn visit(
        schema: &CodecType,
        current: &Heap,
        background: &Heap,
        visited: &mut HashSet<Handle>,
    ) -> Result<(), CodecGraphError> {
        match &schema.kind {
            CodecKind::TypeSlot(handle) => {
                if !visited.insert(*handle) {
                    return Ok(());
                }
                let view = HeapView {
                    current,
                    background: Some(background),
                };
                let resolved = view
                    .type_slot(*handle)
                    .map_err(|error| CodecGraphError::Invalid(error.to_string()))?
                    .ok_or(CodecGraphError::Pending)?;
                let resolved = decode_runtime_type(resolved, current, background)
                    .map_err(CodecGraphError::Invalid)?;
                visit(&resolved, current, background, visited)
            }
            CodecKind::TypeRef(handle) => {
                if !visited.insert(*handle) {
                    return Ok(());
                }
                let view = HeapView {
                    current,
                    background: Some(background),
                };
                let body = match view
                    .object(*handle)
                    .map_err(|error| CodecGraphError::Invalid(error.to_string()))?
                {
                    Object::DeclaredType { body, .. } | Object::SymbolicType { body, .. } => body,
                    _ => return Err(CodecGraphError::Pending),
                };
                let resolved = decode_runtime_type(*body, current, background)
                    .map_err(CodecGraphError::Invalid)?;
                visit(&resolved, current, background, visited)
            }
            CodecKind::Array(item) | CodecKind::Dict(item) => {
                visit(item, current, background, visited)
            }
            CodecKind::Tagged { payload, .. } => visit(payload, current, background, visited),
            CodecKind::Tuple(items) | CodecKind::Union(items) => items
                .iter()
                .try_for_each(|item| visit(item, current, background, visited)),
            CodecKind::Struct(fields) => fields
                .values()
                .try_for_each(|field| visit(field, current, background, visited)),
            CodecKind::Enum(variants) => variants.values().try_for_each(|variant| {
                if let Some(payload) = &variant.payload {
                    visit(payload, current, background, visited)?;
                }
                Ok(())
            }),
            CodecKind::Any
            | CodecKind::Type
            | CodecKind::Dyn
            | CodecKind::Int
            | CodecKind::Float
            | CodecKind::String
            | CodecKind::Bytes
            | CodecKind::Opaque
            | CodecKind::Atom(_)
            | CodecKind::Function => Ok(()),
        }
    }

    visit(schema, current, background, &mut HashSet::new())
}

fn decode_runtime_type_at(
    value: Val,
    path: &str,
    current: &Heap,
    background: &Heap,
) -> Result<CodecType, String> {
    if matches!(value.value(), DecodedValue::NativeType(_)) {
        return Ok(CodecType {
            kind: CodecKind::Opaque,
            rule: value,
            json_rename_all: None,
            json_untagged: None,
            declared_owner: None,
        });
    }
    if let DecodedValue::TypeSlot(handle) = value.value() {
        return Ok(CodecType {
            kind: CodecKind::TypeSlot(handle),
            rule: value,
            json_rename_all: None,
            json_untagged: None,
            declared_owner: None,
        });
    }
    let view = HeapView {
        current,
        background: Some(background),
    };
    if let DecodedValue::DeclaredType(handle) | DecodedValue::SymbolicType(handle) = value.value() {
        return Ok(CodecType {
            kind: CodecKind::TypeRef(handle),
            rule: value,
            json_rename_all: None,
            json_untagged: None,
            declared_owner: None,
        });
    }
    let DecodedValue::Dict(handle) = value.value() else {
        return Err(format!("{path} must be Type metadata"));
    };
    let kind = view
        .dict_get_text(handle, "kind")
        .map_err(|error| error.to_string())?
        .and_then(|kind| view.atom_text(kind).ok().flatten())
        .ok_or_else(|| format!("{path}.kind must be an Atom"))?;
    if kind == "WithAttributes" {
        let fields = view
            .dict_fields(handle)
            .map_err(|error| error.to_string())?;
        if fields != ["attributes", "inner", "kind"] {
            return Err(format!(
                "{path} WithAttributes wrapper must have exactly attributes, inner, and kind fields"
            ));
        }
        let attributes = view
            .dict_get_text(handle, "attributes")
            .map_err(|error| error.to_string())?
            .expect("validated wrapper field");
        let DecodedValue::Dict(attribute_handle) = attributes.value() else {
            return Err(format!("{path}.attributes must be a Dict"));
        };
        let has_attributes = !view
            .dict_parts(attribute_handle)
            .map_err(|error| error.to_string())?
            .0
            .is_empty();
        let inner = view
            .dict_get_text(handle, "inner")
            .map_err(|error| error.to_string())?
            .expect("validated wrapper field");
        let mut decoded = decode_runtime_type_at(inner, path, current, background)?;
        if has_attributes || decoded.rule.loc().is_none() {
            decoded.rule = value;
        }
        return Ok(decoded);
    }
    let kind = match kind.as_str() {
        "Bound" => CodecKind::Any,
        "Named" => CodecKind::Any,
        "Any" => CodecKind::Any,
        "Type" => CodecKind::Type,
        "Dyn" => CodecKind::Dyn,
        "Int" => CodecKind::Int,
        "Float" => CodecKind::Float,
        "String" => CodecKind::String,
        "Bytes" => CodecKind::Bytes,
        "Opaque" => return Err(format!("{path} uses an unsupported opaque type")),
        "Atom" => {
            let tag = view
                .dict_get_text(handle, "tag")
                .map_err(|error| error.to_string())?
                .and_then(|tag| view.atom_text(tag).ok().flatten())
                .ok_or_else(|| format!("{path}.tag must be an Atom"))?;
            CodecKind::Atom(tag.as_str().to_owned())
        }
        "Array" => {
            let item = view
                .dict_get_text(handle, "item")
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("{path}.item is missing"))?;
            CodecKind::Array(Box::new(decode_runtime_type_at(
                item,
                &format!("{path}.item"),
                current,
                background,
            )?))
        }
        "Dict" => {
            let item = view
                .dict_get_text(handle, "item")
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("{path}.item is missing"))?;
            CodecKind::Dict(Box::new(decode_runtime_type_at(
                item,
                &format!("{path}.item"),
                current,
                background,
            )?))
        }
        "Tagged" => {
            let tag = view
                .dict_get_text(handle, "tag")
                .map_err(|error| error.to_string())?
                .and_then(|tag| view.atom_text(tag).ok().flatten())
                .ok_or_else(|| format!("{path}.tag must be an Atom"))?;
            let payload = view
                .dict_get_text(handle, "payload")
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("{path}.payload is missing"))?;
            CodecKind::Tagged {
                tag: tag.as_str().to_owned(),
                payload: Box::new(decode_runtime_type_at(
                    payload,
                    &format!("{path}.payload"),
                    current,
                    background,
                )?),
            }
        }
        "Tuple" | "Union" => {
            let field = if kind == "Tuple" { "items" } else { "variants" };
            let items = view
                .dict_get_text(handle, field)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("{path}.{field} is missing"))?;
            let DecodedValue::Array(items) = items.value() else {
                return Err(format!("{path}.{field} must be an Array"));
            };
            let items = view
                .sequence(items, false)
                .map_err(|error| error.to_string())?
                .to_vec();
            let decoded = items
                .into_iter()
                .enumerate()
                .map(|(index, item)| {
                    decode_runtime_type_at(
                        item,
                        &format!("{path}.{field}[{index}]"),
                        current,
                        background,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            if kind == "Tuple" {
                CodecKind::Tuple(decoded)
            } else {
                CodecKind::Union(decoded)
            }
        }
        "Struct" => {
            let fields = view
                .dict_get_text(handle, "fields")
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("{path}.fields is missing"))?;
            let DecodedValue::Dict(fields) = fields.value() else {
                return Err(format!("{path}.fields must be a Dict"));
            };
            let (names, values) = view.dict_parts(fields).map_err(|error| error.to_string())?;
            let entries = names
                .iter()
                .zip(values)
                .map(|(name, value)| Ok((view.text(*name)?.to_owned(), *value)))
                .collect::<Result<Vec<_>, crate::heap::HeapError>>()
                .map_err(|error| error.to_string())?;
            CodecKind::Struct(
                entries
                    .into_iter()
                    .map(|(name, value)| {
                        let field = decode_runtime_type_at(
                            value,
                            &format!("{path}.fields.{name}"),
                            current,
                            background,
                        )?;
                        Ok((name, field))
                    })
                    .collect::<Result<_, String>>()?,
            )
        }
        "Enum" => {
            let variants = view
                .dict_get_text(handle, "variants")
                .map_err(|error| error.to_string())?
                .ok_or_else(|| format!("{path}.variants is missing"))?;
            let DecodedValue::Dict(variants) = variants.value() else {
                return Err(format!("{path}.variants must be a Dict"));
            };
            let (names, values) = view
                .dict_parts(variants)
                .map_err(|error| error.to_string())?;
            if names.is_empty() {
                return Err(format!("{path}.variants must not be empty"));
            }
            let mut decoded = BTreeMap::new();
            for (name, variant) in names.iter().zip(values) {
                let name = view.text(*name).map_err(|error| error.to_string())?;
                let variant_path = format!("{path}.variants.{name}");
                let inner = strip_runtime_attributes(*variant, &variant_path, &view)?;
                let payload = if view
                    .atom_text(inner)
                    .map_err(|error| error.to_string())?
                    .is_some_and(|atom| atom == "None")
                {
                    None
                } else {
                    Some(Box::new(decode_runtime_type_at(
                        inner,
                        &variant_path,
                        current,
                        background,
                    )?))
                };
                decoded.insert(
                    name.to_owned(),
                    CodecEnumVariant {
                        payload,
                        rule: *variant,
                    },
                );
            }
            CodecKind::Enum(decoded)
        }
        "Func" => CodecKind::Function,
        other => return Err(format!("{path}.kind has unsupported value '{other}")),
    };
    Ok(CodecType {
        kind,
        rule: value,
        json_rename_all: None,
        json_untagged: None,
        declared_owner: None,
    })
}

fn strip_runtime_attributes(
    mut value: Val,
    path: &str,
    view: &HeapView<'_>,
) -> Result<Val, String> {
    while let DecodedValue::Dict(handle) = value.value() {
        let kind = view
            .dict_get_text(handle, "kind")
            .map_err(|error| error.to_string())?
            .and_then(|kind| view.atom_text(kind).ok().flatten());
        if !kind.is_some_and(|kind| kind == "WithAttributes") {
            break;
        }
        let fields = view
            .dict_fields(handle)
            .map_err(|error| error.to_string())?;
        if fields != ["attributes", "inner", "kind"] {
            return Err(format!(
                "{path} WithAttributes wrapper must have exactly attributes, inner, and kind fields"
            ));
        }
        let attributes = view
            .dict_get_text(handle, "attributes")
            .map_err(|error| error.to_string())?
            .expect("validated wrapper field");
        let DecodedValue::Dict(_) = attributes.value() else {
            return Err(format!("{path}.attributes must be a Dict"));
        };
        value = view
            .dict_get_text(handle, "inner")
            .map_err(|error| error.to_string())?
            .expect("validated wrapper field");
    }
    Ok(value)
}

fn option_item(schema: &CodecType) -> Option<&CodecType> {
    if let CodecKind::Enum(variants) = &schema.kind {
        if variants.len() == 2
            && variants
                .get("None")
                .is_some_and(|variant| variant.payload.is_none())
        {
            return variants
                .get("Some")
                .and_then(|variant| variant.payload.as_ref())
                .map(Box::as_ref);
        }
        return None;
    }
    let CodecKind::Union(variants) = &schema.kind else {
        return None;
    };
    if variants.len() != 2 {
        return None;
    }
    let none = variants
        .iter()
        .any(|variant| matches!(&variant.kind, CodecKind::Atom(tag) if tag == "None"));
    let some = variants.iter().find_map(|variant| {
        let CodecKind::Tagged { tag, payload } = &variant.kind else {
            return None;
        };
        (tag == "Some").then_some(payload.as_ref())
    });
    none.then_some(some).flatten()
}

fn is_bool_enum(variants: &BTreeMap<String, CodecEnumVariant>) -> bool {
    variants.len() == 2
        && variants
            .get("False")
            .is_some_and(|variant| variant.payload.is_none())
        && variants
            .get("True")
            .is_some_and(|variant| variant.payload.is_none())
}

fn text_codec_bridge(
    metadata: ValueRef<'_>,
    properties: &CodecProperties,
) -> Result<bool, String> {
    let decode = metadata.type_property(properties.decode_by_parse);
    let encode = metadata.type_property(properties.encode_by_display);
    if decode.is_none() && encode.is_none() {
        return Ok(false);
    }
    if decode.is_none() || encode.is_none() {
        return Err(
            "std/string.decode_by_parse and std/string.encode_by_display must be used together"
                .into(),
        );
    }
    Ok(true)
}

fn parsed_codec_node(value: crate::regex::ParsedValue, loc: Option<crate::Loc>) -> CodecNode {
    match value {
        crate::regex::ParsedValue::String(value) => CodecNode::String(value, loc),
        crate::regex::ParsedValue::Int(value) => {
            CodecNode::Existing(Val::new(DecodedValue::Int(value), loc))
        }
        crate::regex::ParsedValue::Float(value) => {
            CodecNode::Existing(Val::new(DecodedValue::Float(value), loc))
        }
        crate::regex::ParsedValue::None => CodecNode::Atom(BuiltinAtom::None, loc),
        crate::regex::ParsedValue::Some(value) => CodecNode::Tagged {
            tag: Box::new(CodecNode::Atom(BuiltinAtom::Some, loc)),
            payload: Box::new(parsed_codec_node(*value, loc)),
            loc,
        },
        crate::regex::ParsedValue::Struct(fields) => CodecNode::Dict(
            fields
                .into_iter()
                .map(|(name, value)| (name, parsed_codec_node(value, loc)))
                .collect(),
            loc,
        ),
    }
}
