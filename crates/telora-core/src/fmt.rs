use crate::{CallContext, NativeError, NativeType, ValueRef};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
enum TemplatePart {
    Text(String),
    Field(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DisplayTemplate(Vec<TemplatePart>);

#[derive(Clone)]
enum DisplayPlan {
    String,
    Int,
    Float,
    Template {
        template: DisplayTemplate,
        fields: BTreeMap<String, DisplayPlan>,
    },
}

fn template_type(context: &CallContext<'_, '_>) -> Result<NativeType, NativeError> {
    context
        .value(context.upvalue(0)?)?
        .as_native_type()
        .cloned()
        .ok_or_else(|| NativeError::new("DisplayTemplate native type is not linked"))
}

fn resolve(mut metadata: ValueRef<'_>) -> Result<ValueRef<'_>, NativeError> {
    for _ in 0..128 {
        if let Some(body) = metadata.declared_type_body() {
            metadata = body;
            continue;
        }
        if metadata.is_hidden_type_slot() {
            metadata = metadata
                .resolve_hidden_type_slot()
                .map_err(NativeError::new)?;
            continue;
        }
        return Ok(metadata);
    }
    Err(NativeError::new(
        "std/fmt.display metadata resolution exceeds the recursive type limit",
    ))
}

fn strip(mut metadata: ValueRef<'_>) -> Result<ValueRef<'_>, NativeError> {
    metadata = resolve(metadata)?;
    while metadata
        .dict_get("kind")
        .and_then(|kind| kind.as_atom())
        .is_some_and(|kind| kind == "WithAttributes")
    {
        metadata = resolve(
            metadata
                .dict_get("inner")
                .ok_or_else(|| NativeError::new("attributed type has no inner metadata"))?,
        )?;
    }
    Ok(metadata)
}

fn resolve_link(mut metadata: ValueRef<'_>) -> Result<ValueRef<'_>, NativeError> {
    for _ in 0..128 {
        if !metadata.is_hidden_type_slot() {
            return Ok(metadata);
        }
        metadata = metadata
            .resolve_hidden_type_slot()
            .map_err(NativeError::new)?;
    }
    Err(NativeError::new(
        "std/fmt.display type link exceeds the recursive type limit",
    ))
}

fn attached_template(
    metadata: ValueRef<'_>,
    property_type: crate::TypeId,
) -> Result<Option<DisplayTemplate>, NativeError> {
    let mut metadata = resolve_link(metadata)?;
    while metadata
        .dict_get("kind")
        .and_then(|kind| kind.as_atom())
        .is_some_and(|kind| kind == "WithAttributes")
    {
        metadata = resolve_link(
            metadata
                .dict_get("inner")
                .ok_or_else(|| NativeError::new("attributed type has no inner metadata"))?,
        )?;
    }
    let Some(property) = metadata.type_property(property_type) else {
        return Ok(None);
    };
    let payload = property
        .dict_get("template")
        .ok_or_else(|| NativeError::new("fmt DisplayBy property has no template"))?;
    let native_type = payload
        .opaque_native_type()
        .ok_or_else(|| NativeError::new("invalid fmt DisplayBy template"))?;
    if native_type.qualified_name() != "std/fmt#DisplayTemplate" {
        return Err(NativeError::new("invalid fmt DisplayBy template"));
    }
    payload
        .as_opaque::<DisplayTemplate>(native_type)
        .cloned()
        .map(Some)
        .ok_or_else(|| NativeError::new("invalid fmt DisplayBy template"))
}

fn parse_template(source: &str) -> Result<DisplayTemplate, NativeError> {
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut chars = source.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        match ch {
            '{' if chars.peek().is_some_and(|(_, next)| *next == '{') => {
                chars.next();
                text.push('{');
            }
            '}' if chars.peek().is_some_and(|(_, next)| *next == '}') => {
                chars.next();
                text.push('}');
            }
            '{' => {
                if !text.is_empty() {
                    parts.push(TemplatePart::Text(std::mem::take(&mut text)));
                }
                let mut field = String::new();
                loop {
                    match chars.next() {
                        Some((_, '}')) => break,
                        Some((_, '{')) => {
                            return Err(NativeError::new("nested '{' in Display template field"));
                        }
                        Some((_, ch)) => field.push(ch),
                        None => return Err(NativeError::new("unclosed Display template field")),
                    }
                }
                if field.is_empty()
                    || !field
                        .chars()
                        .all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
                    || field.starts_with(|ch: char| ch.is_ascii_digit())
                {
                    return Err(NativeError::new(format!(
                        "invalid Display template field {field:?}"
                    )));
                }
                parts.push(TemplatePart::Field(field));
            }
            '}' => return Err(NativeError::new("unmatched '}' in Display template")),
            ch => text.push(ch),
        }
    }
    if !text.is_empty() {
        parts.push(TemplatePart::Text(text));
    }
    Ok(DisplayTemplate(parts))
}

fn display_plan(
    metadata: ValueRef<'_>,
    property_type: crate::TypeId,
) -> Result<DisplayPlan, NativeError> {
    display_plan_at(metadata, property_type, 0)
}

fn display_plan_at(
    metadata: ValueRef<'_>,
    property_type: crate::TypeId,
    depth: usize,
) -> Result<DisplayPlan, NativeError> {
    if depth >= 128 {
        return Err(NativeError::new(
            "std/fmt.display plan exceeds the recursive type limit",
        ));
    }
    if let Some(template) = attached_template(metadata, property_type)? {
        return display_template_plan(metadata, template, property_type, depth);
    }
    let kind = strip(metadata)?
        .dict_get("kind")
        .and_then(|kind| kind.as_atom());
    match kind.as_ref().map(crate::TextRef::as_str) {
        Some("String") => Ok(DisplayPlan::String),
        Some("Int") => Ok(DisplayPlan::Int),
        Some("Float") => Ok(DisplayPlan::Float),
        _ => Err(NativeError::new("type has no std/fmt.display capability")),
    }
}

fn display_template_plan(
    metadata: ValueRef<'_>,
    template: DisplayTemplate,
    property_type: crate::TypeId,
    depth: usize,
) -> Result<DisplayPlan, NativeError> {
    let inner = strip(metadata)?;
    if !inner
        .dict_get("kind")
        .and_then(|kind| kind.as_atom())
        .is_some_and(|kind| kind == "Struct")
    {
        return Err(NativeError::new("fmt.display_by requires a struct type"));
    }
    let members = inner
        .dict_get("fields")
        .ok_or_else(|| NativeError::new("struct type metadata has no fields"))?;
    let mut fields = BTreeMap::new();
    for part in &template.0 {
        let TemplatePart::Field(name) = part else {
            continue;
        };
        if fields.contains_key(name) {
            continue;
        }
        let field = members.dict_get(name).ok_or_else(|| {
            NativeError::new(format!(
                "Display template references unknown field {name:?}"
            ))
        })?;
        fields.insert(
            name.clone(),
            display_plan_at(field, property_type, depth + 1).map_err(|error| {
                NativeError::new(format!("Display field {name:?}: {}", error.message))
            })?,
        );
    }
    Ok(DisplayPlan::Template { template, fields })
}

fn render(plan: &DisplayPlan, value: ValueRef<'_>, output: &mut String) -> Result<(), NativeError> {
    let value = value
        .unwrap_declared()
        .ok_or_else(|| NativeError::new("Display received an invalid declared value"))?;
    match plan {
        DisplayPlan::String => {
            let text = value
                .as_str()
                .ok_or_else(|| NativeError::new("Display expected String"))?;
            output.push_str(text.as_str());
        }
        DisplayPlan::Int => output.push_str(
            &value
                .as_int()
                .ok_or_else(|| NativeError::new("Display expected Int"))?
                .to_string(),
        ),
        DisplayPlan::Float => output.push_str(
            &value
                .as_float()
                .ok_or_else(|| NativeError::new("Display expected Float"))?
                .to_string(),
        ),
        DisplayPlan::Template { template, fields } => {
            for part in &template.0 {
                match part {
                    TemplatePart::Text(text) => output.push_str(text),
                    TemplatePart::Field(name) => render(
                        &fields[name],
                        value.dict_get(name).ok_or_else(|| {
                            NativeError::new(format!("Display value has no field {name:?}"))
                        })?,
                        output,
                    )?,
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn native_prepare(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = template_type(context)?;
    let source = context
        .value(context.argument(0)?)?
        .as_str()
        .ok_or_else(|| NativeError::new("std/fmt.display_by expects String"))?;
    let template = parse_template(source.as_str())?;

    let property_type = context
        .value(context.argument(1)?)?
        .declared_type_id()
        .ok_or_else(|| NativeError::new("std/fmt.prepare expects DisplayBy Type metadata"))?;
    let target = context.value(context.argument(2)?)?;
    display_template_plan(target, template.clone(), property_type, 0)?;

    let result = context.result();
    context.set_opaque(result, native_type, template)
}

pub(crate) fn native_display(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let property_type = context
        .value(context.argument(0)?)?
        .declared_type_id()
        .ok_or_else(|| NativeError::new("std/fmt.render expects DisplayBy Type metadata"))?;
    let output = display_value(
        context.value(context.argument(1)?)?,
        context.value(context.argument(2)?)?,
        Some(property_type),
    )?;
    context.set_string(context.result(), output)
}

pub(crate) fn display_value(
    metadata: ValueRef<'_>,
    value: ValueRef<'_>,
    property_type: Option<crate::TypeId>,
) -> Result<String, NativeError> {
    let plan = if let Some(property_type) = property_type {
        display_plan(metadata, property_type)?
    } else {
        let kind = strip(metadata)?
            .dict_get("kind")
            .and_then(|kind| kind.as_atom());
        match kind.as_ref().map(crate::TextRef::as_str) {
            Some("String") => DisplayPlan::String,
            Some("Int") => DisplayPlan::Int,
            Some("Float") => DisplayPlan::Float,
            _ => return Err(NativeError::new("type has no std/fmt.display capability")),
        }
    };
    let mut output = String::new();
    render(&plan, value, &mut output)?;
    Ok(output)
}
