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
    if let Some(body) = metadata.declared_type_body() {
        metadata = body;
    }
    if metadata.is_hidden_up_link() {
        metadata = metadata
            .resolve_hidden_up_link()
            .map_err(NativeError::new)?;
    }
    Ok(metadata)
}

fn strip(mut metadata: ValueRef<'_>) -> Result<ValueRef<'_>, NativeError> {
    metadata = resolve(metadata)?;
    while metadata.dict_get("kind").and_then(|kind| kind.as_atom()) == Some("WithAttributes") {
        metadata = resolve(
            metadata
                .dict_get("inner")
                .ok_or_else(|| NativeError::new("attributed type has no inner metadata"))?,
        )?;
    }
    Ok(metadata)
}

fn attached_template(mut metadata: ValueRef<'_>) -> Result<Option<DisplayTemplate>, NativeError> {
    loop {
        metadata = resolve(metadata)?;
        if let Some(provider) = metadata
            .dict_get("attributes")
            .and_then(|attributes| attributes.dict_get("std/fmt.display"))
        {
            let (tag, payload) = provider
                .tagged_parts()
                .ok_or_else(|| NativeError::new("std/fmt.display provider must be Tagged"))?;
            if tag.as_atom() != Some("Template") {
                return Err(NativeError::new("unknown std/fmt.display provider"));
            }
            let native_type = payload
                .opaque_native_type()
                .ok_or_else(|| NativeError::new("invalid std/fmt.display template provider"))?;
            if native_type.qualified_name() != "std/fmt#DisplayTemplate" {
                return Err(NativeError::new(
                    "invalid std/fmt.display template provider",
                ));
            }
            return payload
                .as_opaque::<DisplayTemplate>(native_type)
                .cloned()
                .map(Some)
                .ok_or_else(|| NativeError::new("invalid std/fmt.display template provider"));
        }
        if metadata.dict_get("kind").and_then(|kind| kind.as_atom()) != Some("WithAttributes") {
            return Ok(None);
        }
        metadata = metadata
            .dict_get("inner")
            .ok_or_else(|| NativeError::new("attributed type has no inner metadata"))?;
    }
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

fn display_plan(metadata: ValueRef<'_>) -> Result<DisplayPlan, NativeError> {
    display_plan_at(metadata, 0)
}

fn display_plan_at(metadata: ValueRef<'_>, depth: usize) -> Result<DisplayPlan, NativeError> {
    if depth >= 128 {
        return Err(NativeError::new(
            "std/fmt.display plan exceeds the recursive type limit",
        ));
    }
    if let Some(template) = attached_template(metadata)? {
        let inner = strip(metadata)?;
        if inner.dict_get("kind").and_then(|kind| kind.as_atom()) != Some("Struct") {
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
                display_plan_at(field, depth + 1).map_err(|error| {
                    NativeError::new(format!("Display field {name:?}: {}", error.message))
                })?,
            );
        }
        return Ok(DisplayPlan::Template { template, fields });
    }
    match strip(metadata)?
        .dict_get("kind")
        .and_then(|kind| kind.as_atom())
    {
        Some("String") => Ok(DisplayPlan::String),
        Some("Int") => Ok(DisplayPlan::Int),
        Some("Float") => Ok(DisplayPlan::Float),
        _ => Err(NativeError::new("type has no std/fmt.display capability")),
    }
}

fn render(plan: &DisplayPlan, value: ValueRef<'_>, output: &mut String) -> Result<(), NativeError> {
    let value = value
        .unwrap_declared()
        .ok_or_else(|| NativeError::new("Display received an invalid declared value"))?;
    match plan {
        DisplayPlan::String => output.push_str(
            value
                .as_str()
                .ok_or_else(|| NativeError::new("Display expected String"))?,
        ),
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
    let template = parse_template(source)?;

    let opaque = context.scratch()?;
    context.set_opaque(opaque, native_type.clone(), template)?;
    let tag = context.scratch()?;
    context.set_atom(tag, "Template")?;
    let provider = context.scratch()?;
    context.make_tagged(provider, tag, opaque)?;
    let attributes = context.scratch()?;
    context.make_dict(attributes, &[("std/fmt.display".into(), provider)])?;
    let kind = context.scratch()?;
    context.set_atom(kind, "WithAttributes")?;
    let inner = context.scratch()?;
    context.copy(inner, context.argument(2)?)?;
    let result = context.result();
    context.make_dict(
        result,
        &[
            ("kind".into(), kind),
            ("inner".into(), inner),
            ("attributes".into(), attributes),
        ],
    )?;
    display_plan(context.value(result)?)?;
    Ok(())
}

pub(crate) fn native_display(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let output = display_value(
        context.value(context.argument(0)?)?,
        context.value(context.argument(1)?)?,
    )?;
    context.set_string(context.result(), output)
}

pub(crate) fn display_value(
    metadata: ValueRef<'_>,
    value: ValueRef<'_>,
) -> Result<String, NativeError> {
    let plan = display_plan(metadata)?;
    let mut output = String::new();
    render(&plan, value, &mut output)?;
    Ok(output)
}
