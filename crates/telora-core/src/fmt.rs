use crate::{CallContext, NativeError, NativeType, ValueRef};
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq)]
enum TemplatePart {
    Text(String),
    Field(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DisplayTemplate(Vec<TemplatePart>);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Fmt(Arc<FmtNode>);

#[derive(Debug, Eq, PartialEq)]
enum FmtNode {
    String(String),
    Int(i64),
    Float(u64),
    Atom(String),
    Concat {
        strings: Vec<String>,
        items: Vec<Fmt>,
    },
}

impl Fmt {
    fn string(value: String) -> Self {
        Self(Arc::new(FmtNode::String(value)))
    }

    fn int(value: i64) -> Self {
        Self(Arc::new(FmtNode::Int(value)))
    }

    fn float(value: f64) -> Self {
        Self(Arc::new(FmtNode::Float(value.to_bits())))
    }

    fn atom(value: String) -> Self {
        Self(Arc::new(FmtNode::Atom(value)))
    }

    fn concat(strings: Vec<String>, items: Vec<Self>) -> Self {
        Self(Arc::new(FmtNode::Concat { strings, items }))
    }
}

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

fn fmt_type(context: &CallContext<'_, '_>) -> Result<NativeType, NativeError> {
    context
        .value(context.upvalue(0)?)?
        .as_native_type()
        .cloned()
        .ok_or_else(|| NativeError::new("Fmt native type is not linked"))
}

fn fmt_argument(context: &CallContext<'_, '_>, index: usize) -> Result<Fmt, NativeError> {
    let value = context.value(context.argument(index)?)?;
    let native_type = value
        .opaque_native_type()
        .ok_or_else(|| NativeError::new("expected std/fmt#Fmt"))?;
    if native_type.qualified_name() != "std/fmt#Fmt" {
        return Err(NativeError::new("expected std/fmt#Fmt"));
    }
    value
        .as_opaque::<Fmt>(native_type)
        .cloned()
        .ok_or_else(|| NativeError::new("expected std/fmt#Fmt"))
}

fn write_fmt(value: &Fmt, output: &mut String, depth: usize) -> Result<(), NativeError> {
    if depth >= 128 {
        return Err(NativeError::new(
            "std/fmt value exceeds the recursive rendering limit",
        ));
    }
    match value.0.as_ref() {
        FmtNode::String(value) | FmtNode::Atom(value) => output.push_str(value),
        FmtNode::Int(value) => output.push_str(&value.to_string()),
        FmtNode::Float(value) => output.push_str(&f64::from_bits(*value).to_string()),
        FmtNode::Concat { strings, items } => {
            for (index, item) in items.iter().enumerate() {
                output.push_str(&strings[index]);
                write_fmt(item, output, depth + 1)?;
            }
            output.push_str(&strings[items.len()]);
        }
    }
    Ok(())
}

fn rendered(value: &Fmt) -> Result<String, NativeError> {
    let mut output = String::new();
    write_fmt(value, &mut output, 0)?;
    Ok(output)
}

pub(crate) fn write_interpolation_value(
    value: ValueRef<'_>,
    output: &mut String,
) -> Result<(), NativeError> {
    let native_type = value
        .opaque_native_type()
        .ok_or_else(|| NativeError::new("string interpolation expected std/fmt#Fmt"))?;
    if native_type.qualified_name() != "std/fmt#Fmt" {
        return Err(NativeError::new(
            "string interpolation expected std/fmt#Fmt",
        ));
    }
    write_fmt(
        value
            .as_opaque::<Fmt>(native_type)
            .ok_or_else(|| NativeError::new("string interpolation expected std/fmt#Fmt"))?,
        output,
        0,
    )
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

pub(crate) fn native_display_by(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let property_type = context
        .value(context.argument(0)?)?
        .declared_type_id()
        .ok_or_else(|| NativeError::new("std/fmt.render_by expects DisplayBy Type metadata"))?;
    let output = display_value(
        context.value(context.argument(1)?)?,
        context.value(context.argument(2)?)?,
        Some(property_type),
    )?;
    context.set_opaque(context.result(), native_type, Fmt::string(output))
}

pub(crate) fn native_from_string(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let value = context
        .value(context.argument(0)?)?
        .unwrap_declared()
        .and_then(ValueRef::as_str)
        .ok_or_else(|| NativeError::new("std/fmt.from_string expects String"))?
        .to_string();
    context.set_opaque(context.result(), native_type, Fmt::string(value))
}

pub(crate) fn native_from_int(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let value = context
        .value(context.argument(0)?)?
        .unwrap_declared()
        .and_then(ValueRef::as_int)
        .ok_or_else(|| NativeError::new("std/fmt.from_int expects Int"))?;
    context.set_opaque(context.result(), native_type, Fmt::int(value))
}

pub(crate) fn native_from_float(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let value = context
        .value(context.argument(0)?)?
        .unwrap_declared()
        .and_then(ValueRef::as_float)
        .ok_or_else(|| NativeError::new("std/fmt.from_float expects Float"))?;
    context.set_opaque(context.result(), native_type, Fmt::float(value))
}

pub(crate) fn native_from_atom(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let value = context
        .value(context.argument(0)?)?
        .unwrap_declared()
        .and_then(ValueRef::as_atom)
        .ok_or_else(|| NativeError::new("std/fmt.from_atom expects Atom"))?
        .to_string();
    context.set_opaque(context.result(), native_type, Fmt::atom(value))
}

pub(crate) fn native_concat(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let strings = context.value(context.argument(0)?)?;
    let items = context.value(context.argument(1)?)?;
    let string_count = strings
        .sequence_len()
        .ok_or_else(|| NativeError::new("std/fmt.concat expects an Array(String)"))?;
    let item_count = items
        .sequence_len()
        .ok_or_else(|| NativeError::new("std/fmt.concat expects an Array(Fmt)"))?;
    if string_count != item_count.saturating_add(1) {
        return Err(NativeError::new(format!(
            "std/fmt.concat requires strings.len == items.len + 1, got {string_count} and {item_count}"
        )));
    }
    let strings = (0..string_count)
        .map(|index| {
            strings
                .sequence_get(index)
                .and_then(ValueRef::as_str)
                .map(|value| value.to_string())
                .ok_or_else(|| NativeError::new("std/fmt.concat expects an Array(String)"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let items = (0..item_count)
        .map(|index| {
            let value = items
                .sequence_get(index)
                .ok_or_else(|| NativeError::new("std/fmt.concat expects an Array(Fmt)"))?;
            let fmt_type = value
                .opaque_native_type()
                .ok_or_else(|| NativeError::new("std/fmt.concat expects an Array(Fmt)"))?;
            value
                .as_opaque::<Fmt>(fmt_type)
                .filter(|_| fmt_type.qualified_name() == "std/fmt#Fmt")
                .cloned()
                .ok_or_else(|| NativeError::new("std/fmt.concat expects an Array(Fmt)"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    context.set_opaque(context.result(), native_type, Fmt::concat(strings, items))
}

pub(crate) fn native_render(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let value = fmt_argument(context, 0)?;
    let output = rendered(&value)?;
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
