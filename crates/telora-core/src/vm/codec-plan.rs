fn plan_struct(
    schema: &CodecType,
    fields: &BTreeMap<String, CodecType>,
    data: Val,
    path: &str,
    view: &HeapView<'_>,
) -> Result<StructPlan, CodecFailure> {
    let rename_all = match schema.attributes.get("std/json.rename_all").copied() {
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
        let rename = field.attributes.get("std/json.rename").copied();
        let rename = rename
            .map(|rule| {
                view.string_text(rule)
                    .map_err(|error| CodecFailure::new(error.to_string(), data, rule))?
                    .map(|text| text.as_str().to_owned())
                    .ok_or_else(|| {
                        CodecFailure::new(
                            format!("{path}.{internal_name}: rename must be a String"),
                            data,
                            rule,
                        )
                    })
            })
            .transpose()?;
        let flatten_rule = field.attributes.get("std/json.flatten").copied();
        let flatten = if let Some(rule) = flatten_rule {
            if view
                .atom_text(rule)
                .map_err(|error| CodecFailure::new(error.to_string(), data, rule))?
                .is_none_or(|atom| atom != "True")
            {
                return Err(CodecFailure::new(
                    format!("{path}.{internal_name}: flatten must be 'True"),
                    data,
                    rule,
                ));
            }
            true
        } else {
            false
        };
        let default = field.attributes.get("std/json.default").copied();
        if flatten && (rename.is_some() || default.is_some()) {
            let rule = rename
                .and_then(|_| field.attributes.get("std/json.rename").copied())
                .or_else(|| field.attributes.get("std/json.default").copied())
                .unwrap_or(field.rule);
            return Err(CodecFailure::new(
                format!(
                    "{path}.{internal_name}: flatten cannot be combined with rename or default"
                ),
                data,
                rule,
            ));
        }
        let skip = field
            .attributes
            .get("std/json.skip_serializing_if")
            .copied()
            .map(|rule| {
                let policy = view
                    .atom_text(rule)
                    .map_err(|error| CodecFailure::new(error.to_string(), data, rule))?;
                match policy.as_ref().map(crate::TextRef::as_str) {
                    Some("None") => Ok(SkipPolicy::None),
                    Some("False") => Ok(SkipPolicy::False),
                    Some("Empty") => Ok(SkipPolicy::Empty),
                    _ => {
                        let Some(arity) = view.resolved_function_arity(rule).map_err(|error| {
                            CodecFailure::new(error.to_string(), data, rule)
                        })? else {
                            return Err(CodecFailure::new(
                                format!("{path}.{internal_name}: invalid skip_serializing_if policy"),
                                data,
                                rule,
                            ));
                        };
                        if arity != 1 {
                            return Err(CodecFailure::new(
                                format!("{path}.{internal_name}: skip_serializing_if predicate must accept one argument, got {arity}"),
                                data,
                                rule,
                            ));
                        }
                        Ok(SkipPolicy::Function(rule))
                    }
                }
            })
            .transpose()?;
        let config_rule = flatten_rule
            .or_else(|| field.attributes.get("std/json.rename").copied())
            .unwrap_or(field.rule);
        let (external_name, flattened) = if flatten {
            let CodecKind::Struct(nested_fields) = &field_schema.kind else {
                return Err(CodecFailure::new(
                    format!("{path}.{internal_name}: flatten requires Struct metadata"),
                    data,
                    flatten_rule.unwrap_or(field.rule),
                ));
            };
            let nested = plan_struct(
                &field_schema,
                nested_fields,
                data,
                &format!("{path}.{internal_name}"),
                view,
            )?;
            for (name, rule) in struct_plan_external_names(&nested) {
                if external_names.insert(name.clone(), rule).is_some() {
                    return Err(CodecFailure::new(
                        format!("{path}.{name}: duplicate external field name"),
                        data,
                        rule,
                    ));
                }
            }
            (None, Some(Box::new(nested)))
        } else {
            let external = rename.unwrap_or_else(|| {
                if rename_all {
                    lower_camel_case(internal_name)
                } else {
                    internal_name.clone()
                }
            });
            if external_names
                .insert(external.clone(), config_rule)
                .is_some()
            {
                return Err(CodecFailure::new(
                    format!("{path}.{external}: duplicate external field name"),
                    data,
                    config_rule,
                ));
            }
            (Some(external), None)
        };
        planned.push(StructFieldPlan {
            internal_name: internal_name.clone(),
            external_name,
            schema: field_schema,
            flattened,
            default,
            skip,
            config_rule,
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
    resolved.attributes.extend(schema.attributes.clone());
    Ok(resolved)
}

fn struct_plan_external_names(plan: &StructPlan) -> Vec<(String, Val)> {
    plan.fields
        .iter()
        .flat_map(|field| {
            if let Some(nested) = &field.flattened {
                struct_plan_external_names(nested)
            } else {
                vec![(
                    field.external_name.clone().expect("ordinary field name"),
                    field.config_rule,
                )]
            }
        })
        .collect()
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
    let untagged = match schema.attributes.get("std/json.untagged").copied() {
        Some(rule) => {
            if view
                .atom_text(rule)
                .map_err(|error| CodecFailure::new(error.to_string(), data, rule))?
                .is_none_or(|atom| atom != "True")
            {
                return Err(CodecFailure::new(
                    format!("{path}: untagged must be 'True"),
                    data,
                    rule,
                ));
            }
            true
        }
        None => false,
    };
    let rename_all = match schema.attributes.get("std/json.rename_all").copied() {
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
    for (internal_name, variant) in variants {
        let rename_rule = variant.attributes.get("std/json.rename").copied();
        if let (true, Some(rule)) = (untagged, rename_rule) {
            return Err(CodecFailure::new(
                format!("{path}.{internal_name}: rename is not meaningful in an untagged Enum"),
                data,
                rule,
            ));
        }
        if untagged && variant.payload.is_none() {
            return Err(CodecFailure::new(
                format!("{path}.{internal_name}: untagged variants require payloads"),
                data,
                variant.rule,
            ));
        }
        let external_name = if let Some(rule) = rename_rule {
            view.string_text(rule)
                .map_err(|error| CodecFailure::new(error.to_string(), data, rule))?
                .map(|text| text.as_str().to_owned())
                .ok_or_else(|| {
                    CodecFailure::new(
                        format!("{path}.{internal_name}: rename must be a String"),
                        data,
                        rule,
                    )
                })?
        } else if rename_all {
            lower_camel_case(internal_name)
        } else {
            internal_name.clone()
        };
        if !untagged && names.insert(external_name.clone(), variant.rule).is_some() {
            return Err(CodecFailure::new(
                format!("{path}.{external_name}: duplicate external variant name"),
                data,
                rename_rule.unwrap_or(variant.rule),
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

