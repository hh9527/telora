fn plan_struct(
    schema: &CodecType,
    fields: &BTreeMap<String, CodecType>,
    data: Val,
    path: &str,
    view: &HeapView<'_>,
) -> Result<StructPlan, CodecFailure> {
    let rename_all = match schema.json_rename_all {
        Some(rule) => {
            if view
                .atom_text(rule)
                .map_err(|error| CodecFailure::new(error.to_string(), data, rule))?
                .is_none_or(|atom| atom != "CamelCase")
            {
                return Err(CodecFailure::new(
                    format!("{path}: rename_all must be 'CamelCase"),
                    data,
                    rule,
                ));
            }
            true
        }
        None => false,
    };
    let mut planned = Vec::with_capacity(fields.len());
    let mut external_names: BTreeMap<String, Val> = BTreeMap::new();
    for (internal_name, field) in fields {
        let mut field_schema = resolve_codec_type_once(field, data, view)?;
        if field_schema.rule.loc().is_none() {
            field_schema.rule = schema.rule;
        }
        let external_name = if rename_all {
            lower_camel_case(internal_name)
        } else {
            internal_name.clone()
        };
        if external_names
            .insert(external_name.clone(), field_schema.rule)
            .is_some()
        {
            return Err(CodecFailure::new(
                format!("{path}.{external_name}: duplicate external field name"),
                data,
                field_schema.rule,
            ));
        }
        planned.push(StructFieldPlan {
            internal_name: internal_name.clone(),
            external_name,
            schema: field_schema,
        });
    }
    Ok(StructPlan { fields: planned })
}

fn resolve_codec_type_once(
    schema: &CodecType,
    data: Val,
    view: &HeapView<'_>,
) -> Result<CodecType, CodecFailure> {
    let (resolved, owner) = match schema.kind {
        CodecKind::TypeSlot(handle) => (
            view.type_slot(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), data, schema.rule))?
                .ok_or_else(|| {
                    CodecFailure::new("recursive type link is not initialized", data, schema.rule)
                })?,
            None,
        ),
        CodecKind::TypeRef(handle) => {
            let Object::DeclaredType { body, .. } = view
                .object(handle)
                .map_err(|error| CodecFailure::new(error.to_string(), data, schema.rule))?
            else {
                return Err(CodecFailure::new(
                    "type ref is not sealed",
                    data,
                    schema.rule,
                ));
            };
            (
                *body,
                Some(Val::unknown(DecodedValue::DeclaredType(handle))),
            )
        }
        _ => return Ok(schema.clone()),
    };
    let mut resolved = decode_runtime_type(
        resolved,
        view.current,
        view.background.expect("codec views have a background heap"),
    )
    .map_err(|message| CodecFailure::new(message, data, schema.rule))?;
    resolved.declared_owner = owner;
    if resolved.json_rename_all.is_none() {
        resolved.json_rename_all = schema.json_rename_all;
    }
    if resolved.json_untagged.is_none() {
        resolved.json_untagged = schema.json_untagged;
    }
    Ok(resolved)
}

fn lower_camel_case(name: &str) -> String {
    let mut output = String::with_capacity(name.len());
    let mut uppercase = false;
    for (index, character) in name.chars().enumerate() {
        if character == '_' {
            uppercase = true;
        } else if uppercase {
            output.extend(character.to_uppercase());
            uppercase = false;
        } else if index == 0 {
            output.extend(character.to_lowercase());
        } else {
            output.push(character);
        }
    }
    output
}

#[derive(Clone, Debug)]
struct EnumVariantPlan {
    internal_name: String,
    external_name: String,
    payload: Option<CodecType>,
    rule: Val,
}

#[derive(Clone, Debug)]
struct EnumPlan {
    variants: Vec<EnumVariantPlan>,
    untagged: bool,
}

fn plan_enum(
    schema: &CodecType,
    variants: &BTreeMap<String, CodecEnumVariant>,
    data: Val,
    path: &str,
    view: &HeapView<'_>,
) -> Result<EnumPlan, CodecFailure> {
    let untagged = schema.json_untagged.is_some();
    let rename_all = match schema.json_rename_all {
        Some(rule) => {
            if untagged {
                return Err(CodecFailure::new(
                    format!("{path}: rename_all is not meaningful on an untagged Enum"),
                    data,
                    rule,
                ));
            }
            if view
                .atom_text(rule)
                .map_err(|error| CodecFailure::new(error.to_string(), data, rule))?
                .is_none_or(|atom| atom != "CamelCase")
            {
                return Err(CodecFailure::new(
                    format!("{path}: rename_all must be 'CamelCase"),
                    data,
                    rule,
                ));
            }
            true
        }
        None => false,
    };
    let mut names = BTreeMap::new();
    let mut planned = Vec::with_capacity(variants.len());
    let mut untagged_unit = None;
    for (internal_name, variant) in variants {
        if untagged
            && variant.payload.is_none()
            && untagged_unit.replace(internal_name).is_some()
        {
            return Err(CodecFailure::new(
                format!("{path}: untagged Enum may contain at most one unit variant"),
                data,
                variant.rule,
            ));
        }
        let external_name = if rename_all {
            lower_camel_case(internal_name)
        } else {
            internal_name.clone()
        };
        if !untagged && names.insert(external_name.clone(), variant.rule).is_some() {
            return Err(CodecFailure::new(
                format!("{path}.{external_name}: duplicate external variant name"),
                data,
                variant.rule,
            ));
        }
        planned.push(EnumVariantPlan {
            internal_name: internal_name.clone(),
            external_name,
            payload: variant.payload.as_deref().cloned(),
            rule: variant.rule,
        });
    }
    Ok(EnumPlan {
        variants: planned,
        untagged,
    })
}
