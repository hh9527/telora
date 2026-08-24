use crate::{CallContext, NativeError, NativeType, ValueRef};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::mem::size_of;
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

fn formatted_len(value: impl std::fmt::Display) -> Result<usize, NativeError> {
    struct Counter(Option<usize>);
    impl Write for Counter {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            self.0 = self.0.and_then(|length| length.checked_add(text.len()));
            self.0.map_or(Err(std::fmt::Error), |_| Ok(()))
        }
    }
    let mut counter = Counter(Some(0));
    let _ = write!(&mut counter, "{value}");
    counter
        .0
        .ok_or_else(|| NativeError::allocation_limit("Fmt rendered size overflowed"))
}

fn measure_fmt(value: &Fmt, depth: usize) -> Result<usize, NativeError> {
    if depth >= 128 {
        return Err(NativeError::new(
            "std/fmt value exceeds the recursive rendering limit",
        ));
    }
    let add = |left: usize, right: usize| {
        left.checked_add(right)
            .ok_or_else(|| NativeError::allocation_limit("Fmt rendered size overflowed"))
    };
    match value.0.as_ref() {
        FmtNode::String(value) | FmtNode::Atom(value) => Ok(value.len()),
        FmtNode::Int(value) => formatted_len(value),
        FmtNode::Float(value) => formatted_len(f64::from_bits(*value)),
        FmtNode::Concat { strings, items } => {
            let mut length = 0;
            for (index, item) in items.iter().enumerate() {
                length = add(length, strings[index].len())?;
                length = add(length, measure_fmt(item, depth + 1)?)?;
            }
            add(length, strings[items.len()].len())
        }
    }
}

fn write_fmt(value: &Fmt, output: &mut String, depth: usize) -> Result<(), NativeError> {
    if depth >= 128 {
        return Err(NativeError::new(
            "std/fmt value exceeds the recursive rendering limit",
        ));
    }
    match value.0.as_ref() {
        FmtNode::String(value) | FmtNode::Atom(value) => output.push_str(value),
        FmtNode::Int(value) => write!(output, "{value}").expect("writing to String cannot fail"),
        FmtNode::Float(value) => {
            write!(output, "{}", f64::from_bits(*value)).expect("writing to String cannot fail")
        }
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

pub(crate) fn interpolation_value_len(value: ValueRef<'_>) -> Result<usize, NativeError> {
    if let Some(value) = value.as_str() {
        return Ok(value.as_str().len());
    }
    if let Some(value) = value.as_int() {
        return formatted_len(value);
    }
    if let Some(value) = value.as_float() {
        return formatted_len(value);
    }
    if let Some(value) = value.as_atom() {
        return Ok(value.as_str().len());
    }
    let native_type = value
        .opaque_native_type()
        .ok_or_else(|| NativeError::new("string interpolation expected std/fmt#Fmt"))?;
    if native_type.qualified_name() != "std/fmt#Fmt" {
        return Err(NativeError::new(
            "string interpolation expected std/fmt#Fmt",
        ));
    }
    measure_fmt(
        value
            .as_opaque::<Fmt>(native_type)
            .ok_or_else(|| NativeError::new("string interpolation expected std/fmt#Fmt"))?,
        0,
    )
}

fn fmt_payload_bytes(node: &FmtNode) -> Result<usize, NativeError> {
    let nested = match node {
        FmtNode::String(value) | FmtNode::Atom(value) => value.len(),
        FmtNode::Int(_) | FmtNode::Float(_) => 0,
        FmtNode::Concat { strings, items } => {
            let string_slots = strings
                .len()
                .checked_mul(size_of::<String>())
                .ok_or_else(|| NativeError::allocation_limit("Fmt payload size overflowed"))?;
            let item_slots = items
                .len()
                .checked_mul(size_of::<Fmt>())
                .ok_or_else(|| NativeError::allocation_limit("Fmt payload size overflowed"))?;
            let slots = string_slots
                .checked_add(item_slots)
                .ok_or_else(|| NativeError::allocation_limit("Fmt payload size overflowed"))?;
            strings
                .iter()
                .try_fold(slots, |total, text| total.checked_add(text.len()))
                .ok_or_else(|| NativeError::allocation_limit("Fmt payload size overflowed"))?
        }
    };
    size_of::<Fmt>()
        .checked_add(4 * size_of::<usize>())
        .and_then(|bytes| bytes.checked_add(size_of::<FmtNode>()))
        .and_then(|bytes| bytes.checked_add(nested))
        .ok_or_else(|| NativeError::allocation_limit("Fmt payload size overflowed"))
}

fn set_fmt(
    context: &mut CallContext<'_, '_>,
    native_type: NativeType,
    value: Fmt,
) -> Result<(), NativeError> {
    let payload_bytes = fmt_payload_bytes(value.0.as_ref())?;
    context.set_opaque_accounted(context.result(), native_type, value, payload_bytes)
}

fn template_payload_bytes(template: &DisplayTemplate) -> Result<usize, NativeError> {
    let slots = template
        .0
        .len()
        .checked_mul(size_of::<TemplatePart>())
        .ok_or_else(|| NativeError::allocation_limit("DisplayTemplate payload size overflowed"))?;
    template.0.iter().try_fold(
        size_of::<DisplayTemplate>()
            .checked_add(2 * size_of::<usize>())
            .and_then(|bytes| bytes.checked_add(slots))
            .ok_or_else(|| {
                NativeError::allocation_limit("DisplayTemplate payload size overflowed")
            })?,
        |total, part| {
            let text = match part {
                TemplatePart::Text(text) | TemplatePart::Field(text) => text,
            };
            total.checked_add(text.len()).ok_or_else(|| {
                NativeError::allocation_limit("DisplayTemplate payload size overflowed")
            })
        },
    )
}

pub(crate) fn write_interpolation_value(
    value: ValueRef<'_>,
    output: &mut String,
) -> Result<(), NativeError> {
    if let Some(value) = value.as_str() {
        output.push_str(value.as_str());
        return Ok(());
    }
    if let Some(value) = value.as_int() {
        write!(output, "{value}").expect("writing to String cannot fail");
        return Ok(());
    }
    if let Some(value) = value.as_float() {
        write!(output, "{value}").expect("writing to String cannot fail");
        return Ok(());
    }
    if let Some(value) = value.as_atom() {
        output.push_str(value.as_str());
        return Ok(());
    }
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
    let payload_bytes = template_payload_bytes(&template)?;
    context.set_opaque_accounted(result, native_type, template, payload_bytes)
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
    set_fmt(context, native_type, Fmt::string(output))
}

pub(crate) fn native_from_string(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let value = context
        .value(context.argument(0)?)?
        .unwrap_declared()
        .and_then(ValueRef::as_str)
        .ok_or_else(|| NativeError::new("std/fmt.from_string expects String"))?
        .to_string();
    set_fmt(context, native_type, Fmt::string(value))
}

pub(crate) fn native_from_int(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let value = context
        .value(context.argument(0)?)?
        .unwrap_declared()
        .and_then(ValueRef::as_int)
        .ok_or_else(|| NativeError::new("std/fmt.from_int expects Int"))?;
    set_fmt(context, native_type, Fmt::int(value))
}

pub(crate) fn native_from_float(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let value = context
        .value(context.argument(0)?)?
        .unwrap_declared()
        .and_then(ValueRef::as_float)
        .ok_or_else(|| NativeError::new("std/fmt.from_float expects Float"))?;
    set_fmt(context, native_type, Fmt::float(value))
}

pub(crate) fn native_from_atom(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let value = context
        .value(context.argument(0)?)?
        .unwrap_declared()
        .and_then(ValueRef::as_atom)
        .ok_or_else(|| NativeError::new("std/fmt.from_atom expects Atom"))?
        .to_string();
    set_fmt(context, native_type, Fmt::atom(value))
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
    set_fmt(context, native_type, Fmt::concat(strings, items))
}

pub(crate) fn native_render(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let value = fmt_argument(context, 0)?;
    let length = measure_fmt(&value, 0)?;
    let result = context.result();
    context.set_string_exact(result, length, |output| write_fmt(&value, output, 0))
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
