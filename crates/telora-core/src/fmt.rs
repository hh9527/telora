use crate::{CallContext, NativeError, NativeType, ValueRef};
use std::collections::{BTreeMap, HashMap};
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
    fmt_value(context.value(context.argument(index)?)?).cloned()
}

fn fmt_value(value: ValueRef<'_>) -> Result<&Fmt, NativeError> {
    let native_type = value
        .opaque_native_type()
        .ok_or_else(|| NativeError::new("expected std/fmt#Fmt"))?;
    if native_type.qualified_name() != "std/fmt#Fmt" {
        return Err(NativeError::new("expected std/fmt#Fmt"));
    }
    value
        .as_opaque::<Fmt>(native_type)
        .ok_or_else(|| NativeError::new("expected std/fmt#Fmt"))
}

struct ByteCounter(Option<usize>);

impl Write for ByteCounter {
    fn write_str(&mut self, text: &str) -> std::fmt::Result {
        self.0 = self.0.and_then(|length| length.checked_add(text.len()));
        self.0.map_or(Err(std::fmt::Error), |_| Ok(()))
    }
}

fn formatted_len(value: impl std::fmt::Display) -> Result<usize, NativeError> {
    let mut counter = ByteCounter(Some(0));
    let _ = write!(&mut counter, "{value}");
    counter
        .0
        .ok_or_else(|| NativeError::allocation_limit("Fmt rendered size overflowed"))
}

pub(crate) fn string_with_capacity(length: usize) -> Result<String, NativeError> {
    let mut output = String::new();
    output
        .try_reserve_exact(length)
        .map_err(|_| NativeError::allocation_limit("Fmt String allocation cannot be reserved"))?;
    Ok(output)
}

fn copy_string(value: &str) -> Result<String, NativeError> {
    let mut output = string_with_capacity(value.len())?;
    output.push_str(value);
    Ok(output)
}

#[derive(Clone, Copy)]
struct FmtMeasurement {
    bytes: usize,
    height: usize,
}

fn measure_fmt(value: &Fmt) -> Result<usize, NativeError> {
    fn add(left: usize, right: usize) -> Result<usize, NativeError> {
        left.checked_add(right)
            .ok_or_else(|| NativeError::allocation_limit("Fmt rendered size overflowed"))
    }

    fn visit(
        value: &Fmt,
        memo: &mut HashMap<*const FmtNode, FmtMeasurement>,
    ) -> Result<FmtMeasurement, NativeError> {
        let key = Arc::as_ptr(&value.0);
        if let Some(measurement) = memo.get(&key) {
            return Ok(*measurement);
        }
        let measurement = match value.0.as_ref() {
            FmtNode::String(value) | FmtNode::Atom(value) => FmtMeasurement {
                bytes: value.len(),
                height: 1,
            },
            FmtNode::Int(value) => FmtMeasurement {
                bytes: formatted_len(value)?,
                height: 1,
            },
            FmtNode::Float(value) => FmtMeasurement {
                bytes: formatted_len(f64::from_bits(*value))?,
                height: 1,
            },
            FmtNode::Concat { strings, items } => {
                let mut bytes = strings
                    .iter()
                    .try_fold(0usize, |total, text| add(total, text.len()))?;
                let mut child_height = 0;
                for item in items {
                    let child = visit(item, memo)?;
                    bytes = add(bytes, child.bytes)?;
                    child_height = child_height.max(child.height);
                }
                FmtMeasurement {
                    bytes,
                    height: child_height.checked_add(1).ok_or_else(|| {
                        NativeError::new("std/fmt value exceeds the recursive rendering limit")
                    })?,
                }
            }
        };
        if measurement.height > 128 {
            return Err(NativeError::new(
                "std/fmt value exceeds the recursive rendering limit",
            ));
        }
        memo.insert(key, measurement);
        Ok(measurement)
    }

    Ok(visit(value, &mut HashMap::new())?.bytes)
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
    )
}

fn fmt_payload_bytes(nested: usize) -> Result<usize, NativeError> {
    size_of::<Fmt>()
        .checked_add(4 * size_of::<usize>())
        .and_then(|bytes| bytes.checked_add(size_of::<FmtNode>()))
        .and_then(|bytes| bytes.checked_add(nested))
        .ok_or_else(|| NativeError::allocation_limit("Fmt payload size overflowed"))
}

fn fmt_concat_payload_bytes(
    string_count: usize,
    item_count: usize,
    text_bytes: usize,
) -> Result<usize, NativeError> {
    let string_slots = string_count
        .checked_mul(size_of::<String>())
        .ok_or_else(|| NativeError::allocation_limit("Fmt payload size overflowed"))?;
    let item_slots = item_count
        .checked_mul(size_of::<Fmt>())
        .ok_or_else(|| NativeError::allocation_limit("Fmt payload size overflowed"))?;
    let nested = string_slots
        .checked_add(item_slots)
        .and_then(|bytes| bytes.checked_add(text_bytes))
        .ok_or_else(|| NativeError::allocation_limit("Fmt payload size overflowed"))?;
    fmt_payload_bytes(nested)
}

fn commit_fmt(
    context: &mut CallContext<'_, '_>,
    native_type: NativeType,
    value: Fmt,
    reservation: crate::vm::OpaqueAllocationReservation,
) -> Result<(), NativeError> {
    context.set_opaque_reserved(context.result(), native_type, value, reservation)
}

fn template_payload_bytes(part_count: usize, text_bytes: usize) -> Result<usize, NativeError> {
    let slots = part_count
        .checked_mul(size_of::<TemplatePart>())
        .ok_or_else(|| NativeError::allocation_limit("DisplayTemplate payload size overflowed"))?;
    size_of::<DisplayTemplate>()
        .checked_add(2 * size_of::<usize>())
        .and_then(|bytes| bytes.checked_add(slots))
        .and_then(|bytes| bytes.checked_add(text_bytes))
        .ok_or_else(|| NativeError::allocation_limit("DisplayTemplate payload size overflowed"))
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

#[derive(Clone, Copy)]
struct TemplateMeasurement {
    part_count: usize,
    text_bytes: usize,
}

fn validate_template_field(field: &str) -> Result<(), NativeError> {
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
    Ok(())
}

fn measure_template(source: &str) -> Result<TemplateMeasurement, NativeError> {
    let mut part_count = 0usize;
    let mut text_bytes = 0usize;
    let mut current_text_bytes = 0usize;
    let mut chars = source.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        match ch {
            '{' if chars.peek().is_some_and(|(_, next)| *next == '{') => {
                chars.next();
                current_text_bytes = current_text_bytes.checked_add(1).ok_or_else(|| {
                    NativeError::allocation_limit("DisplayTemplate payload size overflowed")
                })?;
            }
            '}' if chars.peek().is_some_and(|(_, next)| *next == '}') => {
                chars.next();
                current_text_bytes = current_text_bytes.checked_add(1).ok_or_else(|| {
                    NativeError::allocation_limit("DisplayTemplate payload size overflowed")
                })?;
            }
            '{' => {
                if current_text_bytes != 0 {
                    part_count = part_count.checked_add(1).ok_or_else(|| {
                        NativeError::allocation_limit("DisplayTemplate part count overflowed")
                    })?;
                    text_bytes = text_bytes.checked_add(current_text_bytes).ok_or_else(|| {
                        NativeError::allocation_limit("DisplayTemplate payload size overflowed")
                    })?;
                    current_text_bytes = 0;
                }
                let field_start = chars.peek().map_or(source.len(), |(index, _)| *index);
                let field_end = loop {
                    match chars.next() {
                        Some((index, '}')) => break index,
                        Some((_, '{')) => {
                            return Err(NativeError::new("nested '{' in Display template field"));
                        }
                        Some(_) => {}
                        None => return Err(NativeError::new("unclosed Display template field")),
                    }
                };
                let field = &source[field_start..field_end];
                validate_template_field(field)?;
                part_count = part_count.checked_add(1).ok_or_else(|| {
                    NativeError::allocation_limit("DisplayTemplate part count overflowed")
                })?;
                text_bytes = text_bytes.checked_add(field.len()).ok_or_else(|| {
                    NativeError::allocation_limit("DisplayTemplate payload size overflowed")
                })?;
            }
            '}' => return Err(NativeError::new("unmatched '}' in Display template")),
            ch => {
                current_text_bytes =
                    current_text_bytes
                        .checked_add(ch.len_utf8())
                        .ok_or_else(|| {
                            NativeError::allocation_limit("DisplayTemplate payload size overflowed")
                        })?;
            }
        }
    }
    if current_text_bytes != 0 {
        part_count = part_count.checked_add(1).ok_or_else(|| {
            NativeError::allocation_limit("DisplayTemplate part count overflowed")
        })?;
        text_bytes = text_bytes.checked_add(current_text_bytes).ok_or_else(|| {
            NativeError::allocation_limit("DisplayTemplate payload size overflowed")
        })?;
    }
    Ok(TemplateMeasurement {
        part_count,
        text_bytes,
    })
}

fn push_template_part(
    parts: &mut Vec<TemplatePart>,
    part: TemplatePart,
) -> Result<(), NativeError> {
    parts.try_reserve(1).map_err(|_| {
        NativeError::allocation_limit("DisplayTemplate part allocation cannot be reserved")
    })?;
    parts.push(part);
    Ok(())
}

fn push_template_char(text: &mut String, ch: char) -> Result<(), NativeError> {
    text.try_reserve(ch.len_utf8()).map_err(|_| {
        NativeError::allocation_limit("DisplayTemplate text allocation cannot be reserved")
    })?;
    text.push(ch);
    Ok(())
}

fn parse_template(
    source: &str,
    measurement: TemplateMeasurement,
) -> Result<DisplayTemplate, NativeError> {
    let mut parts = Vec::new();
    parts
        .try_reserve_exact(measurement.part_count)
        .map_err(|_| {
            NativeError::allocation_limit("DisplayTemplate part allocation cannot be reserved")
        })?;
    let mut text = String::new();
    let mut chars = source.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        match ch {
            '{' if chars.peek().is_some_and(|(_, next)| *next == '{') => {
                chars.next();
                push_template_char(&mut text, '{')?;
            }
            '}' if chars.peek().is_some_and(|(_, next)| *next == '}') => {
                chars.next();
                push_template_char(&mut text, '}')?;
            }
            '{' => {
                if !text.is_empty() {
                    push_template_part(&mut parts, TemplatePart::Text(std::mem::take(&mut text)))?;
                }
                let mut field = String::new();
                loop {
                    match chars.next() {
                        Some((_, '}')) => break,
                        Some((_, '{')) => {
                            return Err(NativeError::new("nested '{' in Display template field"));
                        }
                        Some((_, ch)) => push_template_char(&mut field, ch)?,
                        None => return Err(NativeError::new("unclosed Display template field")),
                    }
                }
                validate_template_field(&field)?;
                push_template_part(&mut parts, TemplatePart::Field(field))?;
            }
            '}' => return Err(NativeError::new("unmatched '}' in Display template")),
            ch => push_template_char(&mut text, ch)?,
        }
    }
    if !text.is_empty() {
        push_template_part(&mut parts, TemplatePart::Text(text))?;
    }
    debug_assert_eq!(parts.len(), measurement.part_count);
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

fn render(
    plan: &DisplayPlan,
    value: ValueRef<'_>,
    output: &mut impl Write,
) -> Result<(), NativeError> {
    let write_error = || NativeError::allocation_limit("Display output size overflowed");
    let value = value
        .unwrap_declared()
        .ok_or_else(|| NativeError::new("Display received an invalid declared value"))?;
    match plan {
        DisplayPlan::String => {
            let text = value
                .as_str()
                .ok_or_else(|| NativeError::new("Display expected String"))?;
            output.write_str(text.as_str()).map_err(|_| write_error())?;
        }
        DisplayPlan::Int => write!(
            output,
            "{}",
            value
                .as_int()
                .ok_or_else(|| NativeError::new("Display expected Int"))?
        )
        .map_err(|_| write_error())?,
        DisplayPlan::Float => write!(
            output,
            "{}",
            value
                .as_float()
                .ok_or_else(|| NativeError::new("Display expected Float"))?
        )
        .map_err(|_| write_error())?,
        DisplayPlan::Template { template, fields } => {
            for part in &template.0 {
                match part {
                    TemplatePart::Text(text) => {
                        output.write_str(text).map_err(|_| write_error())?
                    }
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

fn measure_display(plan: &DisplayPlan, value: ValueRef<'_>) -> Result<usize, NativeError> {
    let mut counter = ByteCounter(Some(0));
    render(plan, value, &mut counter)?;
    counter
        .0
        .ok_or_else(|| NativeError::allocation_limit("Display output size overflowed"))
}

fn selected_display_plan(
    metadata: ValueRef<'_>,
    property_type: Option<crate::TypeId>,
) -> Result<DisplayPlan, NativeError> {
    if let Some(property_type) = property_type {
        return display_plan(metadata, property_type);
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

pub(crate) fn native_prepare(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = template_type(context)?;
    let source_argument = context.argument(0)?;
    let source = context
        .value(source_argument)?
        .as_str()
        .ok_or_else(|| NativeError::new("std/fmt.display_by expects String"))?;
    let measurement = measure_template(source.as_str())?;
    let payload_bytes = template_payload_bytes(measurement.part_count, measurement.text_bytes)?;
    let reservation = context.reserve_opaque_allocation(payload_bytes)?;
    let source = context
        .value(source_argument)?
        .as_str()
        .expect("String argument was validated before reservation");
    let template = parse_template(source.as_str(), measurement)?;

    let property_type = context
        .value(context.argument(1)?)?
        .declared_type_id()
        .ok_or_else(|| NativeError::new("std/fmt.prepare expects DisplayBy Type metadata"))?;
    let target = context.value(context.argument(2)?)?;
    display_template_plan(target, template.clone(), property_type, 0)?;

    context.set_opaque_reserved(context.result(), native_type, template, reservation)
}

pub(crate) fn native_display_by(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let type_argument = context.argument(1)?;
    let value_argument = context.argument(2)?;
    let property_type = context
        .value(context.argument(0)?)?
        .declared_type_id()
        .ok_or_else(|| NativeError::new("std/fmt.render_by expects DisplayBy Type metadata"))?;
    let plan = selected_display_plan(context.value(type_argument)?, Some(property_type))?;
    let length = measure_display(&plan, context.value(value_argument)?)?;
    let reservation = context.reserve_opaque_allocation(fmt_payload_bytes(length)?)?;
    let mut output = string_with_capacity(length)?;
    render(&plan, context.value(value_argument)?, &mut output)?;
    debug_assert_eq!(output.len(), length);
    commit_fmt(context, native_type, Fmt::string(output), reservation)
}

pub(crate) fn native_from_string(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let argument = context.argument(0)?;
    let length = context
        .value(argument)?
        .unwrap_declared()
        .and_then(ValueRef::as_str)
        .ok_or_else(|| NativeError::new("std/fmt.from_string expects String"))?
        .as_str()
        .len();
    let reservation = context.reserve_opaque_allocation(fmt_payload_bytes(length)?)?;
    let text = context
        .value(argument)?
        .unwrap_declared()
        .and_then(ValueRef::as_str)
        .expect("String argument was validated before reservation");
    let value = copy_string(text.as_str())?;
    commit_fmt(context, native_type, Fmt::string(value), reservation)
}

pub(crate) fn native_from_int(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let value = context
        .value(context.argument(0)?)?
        .unwrap_declared()
        .and_then(ValueRef::as_int)
        .ok_or_else(|| NativeError::new("std/fmt.from_int expects Int"))?;
    let reservation = context.reserve_opaque_allocation(fmt_payload_bytes(0)?)?;
    commit_fmt(context, native_type, Fmt::int(value), reservation)
}

pub(crate) fn native_from_float(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let value = context
        .value(context.argument(0)?)?
        .unwrap_declared()
        .and_then(ValueRef::as_float)
        .ok_or_else(|| NativeError::new("std/fmt.from_float expects Float"))?;
    let reservation = context.reserve_opaque_allocation(fmt_payload_bytes(0)?)?;
    commit_fmt(context, native_type, Fmt::float(value), reservation)
}

pub(crate) fn native_from_atom(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let native_type = fmt_type(context)?;
    let argument = context.argument(0)?;
    let length = context
        .value(argument)?
        .unwrap_declared()
        .and_then(ValueRef::as_atom)
        .ok_or_else(|| NativeError::new("std/fmt.from_atom expects Atom"))?
        .as_str()
        .len();
    let reservation = context.reserve_opaque_allocation(fmt_payload_bytes(length)?)?;
    let text = context
        .value(argument)?
        .unwrap_declared()
        .and_then(ValueRef::as_atom)
        .expect("Atom argument was validated before reservation");
    let value = copy_string(text.as_str())?;
    commit_fmt(context, native_type, Fmt::atom(value), reservation)
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
    let text_bytes = (0..string_count).try_fold(0usize, |total, index| {
        let length = strings
            .sequence_get(index)
            .and_then(ValueRef::as_str)
            .ok_or_else(|| NativeError::new("std/fmt.concat expects an Array(String)"))?
            .as_str()
            .len();
        total
            .checked_add(length)
            .ok_or_else(|| NativeError::allocation_limit("Fmt payload size overflowed"))
    })?;
    for index in 0..item_count {
        let value = items
            .sequence_get(index)
            .ok_or_else(|| NativeError::new("std/fmt.concat expects an Array(Fmt)"))?;
        fmt_value(value).map_err(|_| NativeError::new("std/fmt.concat expects an Array(Fmt)"))?;
    }
    let payload_bytes = fmt_concat_payload_bytes(string_count, item_count, text_bytes)?;
    let reservation = context.reserve_opaque_allocation(payload_bytes)?;

    let strings_value = context.value(context.argument(0)?)?;
    let mut strings = Vec::new();
    strings
        .try_reserve_exact(string_count)
        .map_err(|_| NativeError::allocation_limit("Fmt concat String slots cannot be reserved"))?;
    for index in 0..string_count {
        let value = strings_value
            .sequence_get(index)
            .and_then(ValueRef::as_str)
            .expect("String item was validated before reservation");
        strings.push(copy_string(value.as_str())?);
    }
    let items_value = context.value(context.argument(1)?)?;
    let mut items = Vec::new();
    items
        .try_reserve_exact(item_count)
        .map_err(|_| NativeError::allocation_limit("Fmt concat item slots cannot be reserved"))?;
    for index in 0..item_count {
        items.push(
            fmt_value(
                items_value
                    .sequence_get(index)
                    .expect("Fmt item was validated before reservation"),
            )
            .expect("Fmt item was validated before reservation")
            .clone(),
        );
    }
    commit_fmt(
        context,
        native_type,
        Fmt::concat(strings, items),
        reservation,
    )
}

pub(crate) fn native_render(context: &mut CallContext<'_, '_>) -> Result<(), NativeError> {
    let value = fmt_argument(context, 0)?;
    let length = measure_fmt(&value)?;
    let result = context.result();
    context.set_string_exact(result, length, |output| write_fmt(&value, output, 0))
}

pub(crate) fn display_value(
    metadata: ValueRef<'_>,
    value: ValueRef<'_>,
    property_type: Option<crate::TypeId>,
) -> Result<String, NativeError> {
    let plan = selected_display_plan(metadata, property_type)?;
    let length = measure_display(&plan, value)?;
    let mut output = string_with_capacity(length)?;
    render(&plan, value, &mut output)?;
    debug_assert_eq!(output.len(), length);
    Ok(output)
}
